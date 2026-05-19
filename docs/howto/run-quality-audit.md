# How to Run a Quality Audit

Run the `quality-audit-cycle` recipe to scan a codebase for quality issues with
escalating-depth SEEK/VALIDATE/FIX/RECURSE cycles.

## Prerequisites

- amplihack installed (`AMPLIHACK_HOME` set)
- `amplihack` CLI binary on PATH
- Target repository checked out locally

## Basic Invocation

```bash
amplihack recipe run quality-audit-cycle \
  -c task_description="Run quality audit on the payments module" \
  -c repo_path="." \
  -c target_path="src/payments" \
  --verbose
```

> **Note:** Use `amplihack recipe run` — not `amplihack recipe execute`. The
> CLI is the canonical invocation path, matching how `dev-orchestrator`
> and all other recipes are launched.

## Setting `repo_path`

The `repo_path` variable tells agent steps where the repository root is. Set it
so that `target_path` resolves relative to the repo:

```bash
amplihack recipe run quality-audit-cycle \
  -c task_description="Audit the crates directory" \
  -c repo_path="/home/user/src/my-project" \
  -c target_path="crates/" \
  --verbose
```

When `repo_path` is set, each agent step's `working_dir` is set to that path,
giving agents file-system access to the target directory.

**Rules:**

| `repo_path`             | `target_path`  | Agent sees                             |
| ----------------------- | -------------- | -------------------------------------- |
| `.` (default)           | `src/payments` | `./src/payments` from CWD              |
| `/home/user/src/myproj` | `crates/`      | `crates/` relative to `/home/…/myproj` |
| (omitted)               | absolute path  | Works, but agents may lack CWD context |

## Targeting a Subdirectory

Set `target_path` to audit a specific part of the codebase:

```bash
amplihack recipe run quality-audit-cycle \
  -c target_path="src/amplihack/fleet" \
  -c repo_path="." \
  -c min_cycles="2" \
  -c max_cycles="4" \
  --verbose
```

## Filtering by Category

Limit the audit to specific issue categories:

```bash
amplihack recipe run quality-audit-cycle \
  -c target_path="src/amplihack" \
  -c repo_path="." \
  -c categories="security,reliability,error_swallowing" \
  --verbose
```

Available categories: `security`, `reliability`, `dead_code`, `silent_fallbacks`,
`error_swallowing`, `result_dropping`, `shell_anti_patterns`, `silent_truncation`,
`async_anti_patterns`, `config_divergence`, `validation_gaps`, `health_observability`,
`retry_anti_patterns`, `structural`, `hardcoded_limits`, `test_gaps`, `doc_gaps`,
`documentation`.

## Adjusting Cycle Limits

```bash
amplihack recipe run quality-audit-cycle \
  -c target_path="src/amplihack" \
  -c repo_path="." \
  -c min_cycles="3" \
  -c max_cycles="6" \
  -c severity_threshold="high" \
  --verbose
```

## Troubleshooting

### "target path does not exist"

The agent cannot find `target_path`. Likely causes:

1. **Missing `repo_path`** — set `repo_path` to the repo root so
   agents resolve relative paths correctly.
2. **Relative path with wrong CWD** — ensure your shell's CWD is the repo root
   before running `amplihack recipe run`, or use an absolute `target_path`.

### Bash step errors like `json: command not found`

Template variables (`{{validated_findings}}`) are being interpreted as bash
commands instead of being interpolated. This is a heredoc safety issue —
see the [recipe reference](../reference/quality-audit-cycle-recipe.md)
for details on the fix.

### Recipe completes but agents produce empty results

This is a **hollow success**. Check:

1. `repo_path` points to the actual repo root
2. `target_path` contains files the agents can read
3. The agent binary has access to file-reading tools

### `VERIFY: FAIL — fix-agent claims N files fixed but git diff shows no file modifications`

The `verify-fixes` step cross-checks the fix-agent's JSON output against
`git diff --quiet`. This error means the fix-agent produced a structurally valid
response claiming it fixed files, but no actual file modifications exist in the
working tree.

Common causes:

1. **Fix-agent hallucinated changes** — the agent reported fixes in its JSON
   output but did not actually write to disk. Re-run the cycle; the next
   SEEK will rediscover the unfixed findings.
2. **Changes were staged or committed** — if the fix-agent ran `git add` or
   `git commit`, the unstaged diff will be empty. The verify step checks
   unstaged changes only. This is unusual (fix-agents are not instructed to
   commit) but check with `git log --oneline -3` and `git diff --cached --stat`.
3. **Working directory mismatch** — the fix-agent modified files in a different
   directory than `repo_path`. Verify `repo_path` is correct.

### Recursive cycle times out after 15 minutes

The `run-recursive-cycle` step wraps the subprocess invocation in
`timeout 900` (15 minutes). If a cycle exceeds this limit:

1. The subprocess receives SIGTERM and the recipe halts.
2. Check `eval_results/quality_audit/` for partial output from completed steps.
3. Consider reducing `max_cycles` or narrowing `target_path` to a smaller scope.
4. For large codebases, run separate audits per directory instead of one
   monolithic audit.

### Recursive cycle returns empty output

Prior to #646, the `run-recursive-cycle` step used `type: recipe` with
`sub_context`, which silently dropped the sub-recipe's output. This has been
replaced with a `type: bash` subprocess invocation that captures stdout.

If you still see empty `final_report` output:

1. Check that `amplihack recipe run` is on PATH and functional.
2. Look for errors in stderr — the subprocess pipes stderr through.
3. Verify `AMPLIHACK_HOME` is set (the subprocess needs it to locate recipes).

## See Also

- [Quality Audit Recipe Reference](../reference/quality-audit-cycle-recipe.md) — full
  context variable table and step-by-step reference
- [Quality Audit Skill](../../amplifier-bundle/skills/quality-audit/SKILL.md) — skill
  activation triggers and detection categories
